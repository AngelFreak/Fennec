package io.github.fennec.recorder.data

import androidx.room.AutoMigration
import androidx.room.Dao
import androidx.room.Database
import androidx.room.Entity
import androidx.room.Insert
import androidx.room.OnConflictStrategy
import androidx.room.PrimaryKey
import androidx.room.Query
import androidx.room.RoomDatabase
import androidx.room.Update
import androidx.room.migration.Migration
import androidx.sqlite.db.SupportSQLiteDatabase
import kotlinx.coroutines.flow.Flow

/** Where a recording is on its way to Fennec. */
enum class SyncState {
    /** Being recorded now. */
    RECORDING,

    /** Saved on the phone; not sent yet. */
    WAITING,

    /** Going to Fennec now, after a scan. */
    SENDING,

    /** Fennec has it: `complete` answered. */
    SENT,

    /** Fennec refused it; the next scan tries again. */
    FAILED,

    /** Taken over a USB cable (Fennec removed the phone's copy), or marked
     *  sent by hand. */
    USB;

    /** Fennec has the whole file. */
    val delivered: Boolean get() = this == SENT || this == USB

    /** Goes with the next scan. */
    val unsent: Boolean get() = this == WAITING || this == SENDING || this == FAILED
}

@Entity(tableName = "recordings")
data class Recording(
    /** A UUID, made when recording starts; Fennec knows the recording by it. */
    @PrimaryKey val id: String,
    val title: String,
    /** When recording started, ms since the epoch. */
    val recordedAt: Long,
    val durationMs: Long = 0,
    val projectId: Long? = null,
    val projectName: String? = null,
    val templateId: String? = null,
    val templateName: String? = null,
    /** File name in the app's recordings folder. */
    val fileName: String,
    val ext: String = "aac",
    val size: Long = 0,
    val sha256: String? = null,
    val state: SyncState = SyncState.RECORDING,
    /** Bytes Fennec has confirmed. */
    val sentBytes: Long = 0,
    val documentId: Long? = null,
    val error: String? = null,
    /** When Fennec had all of it. */
    val deliveredAt: Long? = null,
    /** Times the file arrived damaged and was sent again. */
    val resends: Int = 0,
    /** The copy in Download/Fennec Recorder for USB transfer (a MediaStore URI). */
    val usbAudio: String? = null,
    /** Its sidecar with title, project and template. */
    val usbMeta: String? = null,
)

@Dao
interface RecordingDao {
    @Query("SELECT * FROM recordings ORDER BY recordedAt DESC")
    fun observeAll(): Flow<List<Recording>>

    @Query("SELECT * FROM recordings WHERE id = :id")
    fun observe(id: String): Flow<Recording?>

    @Query("SELECT * FROM recordings WHERE id = :id")
    suspend fun get(id: String): Recording?

    /** Everything not sent, failed ones too, oldest first. */
    @Query("SELECT * FROM recordings WHERE state IN ('WAITING', 'SENDING', 'FAILED') ORDER BY recordedAt")
    suspend fun toSend(): List<Recording>

    /** A transfer ended (stopped, or the app was killed): what it had not finished is not sent. */
    @Query("UPDATE recordings SET state = 'WAITING' WHERE state = 'SENDING'")
    suspend fun stopSending()

    /** Not delivered and without a copy for USB transfer. */
    @Query("SELECT * FROM recordings WHERE state IN ('WAITING', 'SENDING', 'FAILED') AND deliveredAt IS NULL AND usbAudio IS NULL")
    suspend fun needingUsbCopy(): List<Recording>

    @Query("SELECT * FROM recordings WHERE usbAudio IS NOT NULL OR usbMeta IS NOT NULL")
    suspend fun withUsbCopy(): List<Recording>

    @Query("SELECT * FROM recordings WHERE state = 'RECORDING'")
    suspend fun unfinished(): List<Recording>

    @Query("SELECT * FROM recordings WHERE state IN ('SENT', 'USB') AND deliveredAt < :before")
    suspend fun deliveredBefore(before: Long): List<Recording>

    @Insert(onConflict = OnConflictStrategy.REPLACE)
    suspend fun put(r: Recording)

    @Update
    suspend fun update(r: Recording)

    @Query("UPDATE recordings SET sentBytes = :sent, state = 'SENDING' WHERE id = :id")
    suspend fun setSent(id: String, sent: Long)

    @Query("DELETE FROM recordings WHERE id = :id")
    suspend fun delete(id: String)
}

@Database(entities = [Recording::class], version = 3, autoMigrations = [AutoMigration(from = 1, to = 2)])
abstract class AppDatabase : RoomDatabase() {
    abstract fun recordings(): RecordingDao

    companion object {
        /**
         * The phone no longer follows transcription (protocol v2): whatever
         * Fennec had (queued, transcribing, transcribed, or failed there) is
         * simply sent. The table itself is unchanged.
         */
        val MIGRATION_2_3 = object : Migration(2, 3) {
            override fun migrate(db: SupportSQLiteDatabase) {
                db.execSQL("UPDATE recordings SET state = 'SENT' WHERE state IN ('QUEUED', 'TRANSCRIBING', 'DONE')")
                db.execSQL("UPDATE recordings SET state = 'SENT', error = NULL WHERE state = 'FAILED' AND deliveredAt IS NOT NULL")
                db.execSQL("UPDATE recordings SET state = 'WAITING' WHERE state = 'SENDING'")
            }
        }
    }
}
